#!/usr/bin/env bash
# Run inside a native-architecture Fedora container; never convert the RPM.
set -euo pipefail
exec > >(tee smoke/installer.log) 2>&1
dnf install -y python3 python3-pillow procps-ng xorg-x11-server-Xvfb xorg-x11-xauth xdotool openbox desktop-file-utils
dnf install -y packages/*.rpm
rpm -V perseid
desktop-file-validate /usr/share/applications/perseid.desktop
EXE=$(command -v perseid)
export EXE
export HOME=/tmp/perseid-smoke
mkdir -p "$HOME"
Xvfb :99 -screen 0 1440x1000x24 > smoke/xvfb.log 2>&1 &
export DISPLAY=:99
trap 'kill $(jobs -pr) 2>/dev/null || true' EXIT
for _ in {1..50}; do
  if xdotool getdisplaygeometry >/dev/null 2>&1; then break; fi
  sleep 0.1
done
openbox > smoke/window-manager.log 2>&1 &
python3 scripts/installer-smoke.py "$EXE" "smoke/$SMOKE_ID.png"
dnf remove -y perseid
test ! -e "$EXE"
