#!/usr/bin/env bash
# Build a native RPM from the same release binary used by the other packages.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
topdir="$PWD/dist/rpmbuild"
mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
# RPM versions cannot contain a hyphen. Preserve prerelease/build identifiers
# using RPM's ordering markers, while keeping the public asset version intact.
rpm_version=${version//-/\~}
rpm_version=${rpm_version//+/\^}
rpmbuild -bb packaging/perseid.spec \
  --define "_topdir $topdir" \
  --define "source_root $PWD" \
  --define "app_version $rpm_version"
python3 scripts/collect-release.py --os linux --arch "${1:?expected x64 or arm64}"
