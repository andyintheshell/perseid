"""Release asset normalization must include both native RPM architectures."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'collect_release', Path(__file__).with_name('collect-release.py'))
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class ReleasePackagingTests(unittest.TestCase):
    def test_collect_all_formats_for_both_architectures(self):
        for arch, expected in [('x64', 'x86_64'), ('arm64', 'aarch64')]:
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                (root / 'Cargo.toml').write_text('[workspace.package]\nversion = "1.2.3"\n')
                for directory, suffix in [('packager', '.deb'), ('packager', '.AppImage'),
                                          ('rpmbuild/RPMS/' + expected, '.rpm')]:
                    folder = root / 'dist' / directory
                    folder.mkdir(parents=True, exist_ok=True)
                    (folder / ('original' + suffix)).write_bytes(b'package')
                with patch.object(collector, 'ROOT', root), patch(
                        'sys.argv', ['collect-release.py', '--os', 'linux', '--arch', arch]):
                    collector.main()
                for suffix in ('.deb', '.AppImage', '.rpm'):
                    self.assertEqual((root / 'dist' / f'perseid-1.2.3-linux-{expected}{suffix}').read_bytes(), b'package')

    def test_missing_or_ambiguous_rpm_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with self.assertRaisesRegex(RuntimeError, 'Expected exactly one'):
                collector.one_file(root, '.rpm')
            for name in ('one.rpm', 'two.rpm'):
                (root / name).touch()
            with self.assertRaisesRegex(RuntimeError, 'Expected exactly one'):
                collector.one_file(root, '.rpm')


class RpmBuildScriptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        self.script = self.root / 'scripts/package-rpm.sh'
        shutil.copyfile(Path(__file__).with_name('package-rpm.sh'), self.script)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'])
        self.command('uname', 'echo x86_64')
        # Avoid depending on Python's executable name in the calling environment.
        self.command('python3', 'echo 1.2.3')
        self.command('rpmbuild', 'touch build-ran')

    def command(self, name, body):
        path = self.bin / name
        path.write_text('#!/bin/sh\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def run_script(self, *args):
        return subprocess.run(['bash', str(self.script), *args], cwd=self.root,
                              env=self.env, capture_output=True, text=True)

    def test_invalid_arguments_fail_before_build(self):
        for args in [(), ('invalid',), ('x64', 'extra'), ('arm64',)]:
            with self.subTest(args=args):
                result = self.run_script(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.root / 'build-ran').exists())
                self.assertFalse((self.root / 'dist').exists())

    def test_stale_rpm_cleanup_is_scoped_and_precedes_build(self):
        output = self.root / 'dist/rpmbuild/RPMS/x86_64'
        output.mkdir(parents=True)
        stale = output / 'old.rpm'
        keep = output / 'keep.txt'
        stale.touch()
        keep.touch()
        self.command('rpmbuild', 'test ! -e dist/rpmbuild/RPMS/x86_64/old.rpm\n'
                     'touch build-ran dist/rpmbuild/RPMS/x86_64/current.rpm')
        result = self.run_script('x64')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / 'build-ran').exists())
        self.assertTrue((output / 'current.rpm').exists())
        self.assertTrue(keep.exists())

    def test_native_arm64_is_accepted(self):
        self.command('uname', 'echo aarch64')
        result = self.run_script('arm64')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / 'build-ran').exists())
