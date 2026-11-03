"""Release asset normalization must include both native RPM architectures."""

import importlib.util
from pathlib import Path
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
