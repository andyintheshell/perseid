import hashlib
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import windows_smoke_opengl as mesa


def pe(machine):
    data = bytearray(128)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<H", data, 68, machine)
    return data


class MesaTests(unittest.TestCase):
    def test_provision_native_architectures(self):
        for machine, (arch, _) in mesa.BUILDS.items():
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as directory:
                exe = Path(directory) / "perseid.exe"
                exe.write_bytes(pe(machine))

                def download(url, destination):
                    self.assertIn(f"mesa-llvmpipe-{arch}-", url)
                    Path(destination).write_bytes(b"archive")

                def extract(command, check):
                    self.assertTrue(check)
                    out = Path(command[3][2:])
                    (out / "opengl32.dll").write_bytes(pe(machine))

                builds = {machine: (arch, hashlib.sha256(b"archive").hexdigest())}
                with (
                    patch.object(mesa, "BUILDS", builds),
                    patch.object(mesa.urllib.request, "urlretrieve", side_effect=download),
                    patch.object(mesa.subprocess, "run", side_effect=extract),
                ):
                    mesa.provision(exe)
                self.assertEqual(mesa.pe_machine(exe.with_name("opengl32.dll")), machine)

    def test_checksum_failure_prevents_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            exe = Path(directory) / "perseid.exe"
            exe.write_bytes(pe(0x8664))
            with (
                patch.object(mesa.urllib.request, "urlretrieve", side_effect=lambda _, p: Path(p).write_bytes(b"bad")),
                patch.object(mesa.subprocess, "run") as extract,
            ):
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    mesa.provision(exe)
                extract.assert_not_called()
            self.assertFalse(exe.with_name("opengl32.dll").exists())

    def test_wrong_dll_architecture_is_not_installed(self):
        with tempfile.TemporaryDirectory() as directory:
            exe = Path(directory) / "perseid.exe"
            exe.write_bytes(pe(0xAA64))

            def extract(command, check):
                (Path(command[3][2:]) / "opengl32.dll").write_bytes(pe(0x8664))

            with (
                patch.object(mesa, "BUILDS", {0xAA64: ("arm64", hashlib.sha256(b"archive").hexdigest())}),
                patch.object(mesa.urllib.request, "urlretrieve", side_effect=lambda _, p: Path(p).write_bytes(b"archive")),
                patch.object(mesa.subprocess, "run", side_effect=extract),
            ):
                with self.assertRaisesRegex(ValueError, "architecture does not match"):
                    mesa.provision(exe)
            self.assertFalse(exe.with_name("opengl32.dll").exists())

    def test_existing_dll_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            exe = Path(directory) / "perseid.exe"
            exe.write_bytes(pe(0x8664))
            dll = exe.with_name("opengl32.dll")
            dll.write_bytes(b"packaged")
            with self.assertRaises(FileExistsError):
                mesa.provision(exe)
            self.assertEqual(dll.read_bytes(), b"packaged")

    def test_rejects_malformed_pe(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.exe"
            for data in (b"", b"not PE", b"MZ" + bytes(100)):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    mesa.pe_machine(path)

    def test_rejects_unsupported_architecture(self):
        with tempfile.TemporaryDirectory() as directory:
            exe = Path(directory) / "perseid.exe"
            exe.write_bytes(pe(0x14C))
            with self.assertRaisesRegex(ValueError, "Unsupported PE machine"):
                mesa.provision(exe)


if __name__ == "__main__":
    unittest.main()
