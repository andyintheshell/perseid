#!/usr/bin/env python3
"""Normalize cargo-packager output names for GitHub Releases."""

import argparse
from pathlib import Path
import shutil
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def one_file(root: Path, suffix: str) -> Path:
    found = [
        path
        for path in root.rglob('*')
        if path.is_file() and path.name.lower().endswith(suffix.lower())
    ]
    if len(found) != 1:
        raise RuntimeError(f'Expected exactly one *{suffix} in {root}; found: {found}')
    return found[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--os', choices=('linux',), required=True)
    parser.add_argument('--arch', choices=('x64', 'arm64'), required=True)
    args = parser.parse_args()

    with (ROOT / 'Cargo.toml').open('rb') as manifest:
        version = tomllib.load(manifest)['workspace']['package']['version']

    packaged = ROOT / 'dist' / 'packager'
    if not packaged.is_dir():
        raise FileNotFoundError(f'cargo-packager output directory not found: {packaged}')

    arch = {'x64': 'x86_64', 'arm64': 'aarch64'}[args.arch]
    stem = f'perseid-{version}-linux-{arch}'
    for suffix in ('.AppImage', '.deb'):
        source = one_file(packaged, suffix)
        destination = ROOT / 'dist' / f'{stem}{suffix}'
        shutil.copy2(source, destination)
        print(f'Created {destination}')


if __name__ == '__main__':
    main()
