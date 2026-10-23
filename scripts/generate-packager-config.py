#!/usr/bin/env python3
"""Generate the Linux AppImage and Debian package configuration."""

import argparse
import json
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--os', choices=('linux',), required=True)
    parser.parse_args()

    with (ROOT / 'Cargo.toml').open('rb') as manifest:
        version = tomllib.load(manifest)['workspace']['package']['version']

    licenses = {
        'LICENSE-MIT': 'usr/share/doc/perseid/LICENSE-MIT',
        'LICENSE-APACHE': 'usr/share/doc/perseid/LICENSE-APACHE',
    }
    config = {
        # Setting the package name avoids cargo-packager 0.11.8 trying to infer
        # it by changing into the config file's path instead of its parent dir.
        'name': 'perseid',
        'productName': 'Perseid',
        'identifier': 'io.github.andyintheshell.perseid',
        'version': version,
        'publisher': 'andyintheshell',
        'description': 'A native HTTP(S) security testing workbench',
        'homepage': 'https://github.com/andyintheshell/perseid',
        'outDir': 'dist/packager',
        'binariesDir': 'target/release',
        'binaries': [{'path': 'perseid', 'main': True}],
        'formats': ['deb', 'appimage'],
        'icons': ['branding/perseid-icon-512.png'],
        'linux': {'generateDesktopEntry': True},
        'deb': {
            'depends': [
                'libc6',
                'libgcc-s1',
                'libegl1',
                'libgl1',
                'libwayland-client0',
                'libwayland-cursor0',
                'libwayland-egl1',
                'libx11-6',
                'libx11-xcb1',
                'libxcb-render0',
                'libxcb-shape0',
                'libxcb-xfixes0',
                'libxcb1',
                'libxcursor1',
                'libxi6',
                'libxkbcommon0',
                'libxkbcommon-x11-0',
                'libxrandr2',
            ],
            'files': licenses,
        },
        'appimage': {'files': licenses},
    }

    output = ROOT / 'packager.json'
    output.write_text(json.dumps(config, indent=2) + '\n', encoding='utf-8')
    print(f'Wrote {output}: {version} / Linux [deb, appimage]')


if __name__ == '__main__':
    main()
