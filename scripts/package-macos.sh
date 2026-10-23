#!/usr/bin/env bash
# Build one universal, ad-hoc-signed Perseid.app inside a drag-to-Applications DMG.
# This is not Developer ID signed or notarized.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
BUNDLE_VERSION="${VERSION%%[-+]*}"
if [[ ! "$BUNDLE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Invalid macOS bundle version derived from $VERSION" >&2
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --locked -p perseid --target aarch64-apple-darwin
cargo build --release --locked -p perseid --target x86_64-apple-darwin

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/dmg/Perseid.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Make an ICNS from the project's existing PNG branding so Finder and the Dock
# show Perseid's icon without maintaining a separate binary asset.
ICONSET="$WORK/perseid.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  double=$((size * 2))
  sips -z "$size" "$size" "$ROOT/branding/perseid-icon-512.png" \
    --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z "$double" "$double" "$ROOT/branding/perseid-icon-512.png" \
    --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/perseid.icns"

lipo -create \
  "$ROOT/target/aarch64-apple-darwin/release/perseid" \
  "$ROOT/target/x86_64-apple-darwin/release/perseid" \
  -output "$APP/Contents/MacOS/perseid"
chmod 755 "$APP/Contents/MacOS/perseid"
lipo -verify_arch arm64 x86_64 "$APP/Contents/MacOS/perseid"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>perseid</string>
<key>CFBundleIdentifier</key><string>io.github.andyintheshell.perseid</string>
<key>CFBundleName</key><string>Perseid</string>
<key>CFBundleDisplayName</key><string>Perseid</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>${BUNDLE_VERSION}</string>
<key>CFBundleVersion</key><string>${BUNDLE_VERSION}</string>
<key>LSMinimumSystemVersion</key><string>${MACOSX_DEPLOYMENT_TARGET}</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleIconFile</key><string>perseid</string>
</dict></plist>
EOF
cp LICENSE-MIT LICENSE-APACHE "$APP/Contents/Resources/"
plutil -lint "$APP/Contents/Info.plist"
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict --verbose=2 "$APP"
ln -s /Applications "$WORK/dmg/Applications"
mkdir -p dist
hdiutil create -ov -format UDZO -volname Perseid \
  -srcfolder "$WORK/dmg" "dist/perseid-${VERSION}-macos-universal.dmg"
echo "Created dist/perseid-${VERSION}-macos-universal.dmg (ad-hoc signed, not notarized)"
