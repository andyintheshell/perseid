#!/usr/bin/env bash
# Build a native RPM from the same release binary used by the other packages.
set -euo pipefail
if [[ $# -ne 1 ]]; then
  echo 'Usage: package-rpm.sh x64|arm64' >&2
  exit 1
fi
case "$1" in
  x64) expected=x86_64 ;;
  arm64) expected=aarch64 ;;
  *) echo "Unsupported architecture: $1" >&2; exit 1 ;;
esac
actual=$(uname -m)
if [[ "$actual" != "$expected" ]]; then
  echo "Host architecture is $actual; expected $expected" >&2
  exit 1
fi
cd "$(dirname "$0")/.."
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
topdir="$PWD/dist/rpmbuild"
mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
# Only remove generated RPM outputs, not other files in the build tree.
find "$topdir/RPMS" -type f -name '*.rpm' -delete
# RPM versions cannot contain a hyphen. Preserve prerelease/build identifiers
# using RPM's ordering markers, while keeping the public asset version intact.
rpm_version=${version//-/\~}
rpm_version=${rpm_version//+/\^}
rpmbuild -bb packaging/perseid.spec \
  --define "_topdir $topdir" \
  --define "source_root $PWD" \
  --define "app_version $rpm_version"
python3 scripts/collect-release.py --os linux --arch "$1"
