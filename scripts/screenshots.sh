#!/usr/bin/env bash
#
# Generate the two documentation screenshots headlessly.
#
# Usage:
#   scripts/screenshots.sh [--out DIR] [--no-build]
#
# The app is built with the `screenshots` feature, which seeds a deterministic
# demo history and captures the egui framebuffer directly. No window manager,
# mouse movement, or screen-grab utility is involved. On success the PNGs are
# copied to DIR (default: docs/images).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/docs/images"
BUILD=1

usage() {
    cat <<'EOF'
Usage: scripts/screenshots.sh [--out DIR] [--no-build]

  --out DIR    Copy the generated PNGs to DIR (default: docs/images)
  --no-build   Use the existing target/debug/perseid build
  -h, --help   Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)
            [[ $# -ge 2 ]] || { echo "--out needs a directory" >&2; exit 2; }
            OUT="$2"
            shift 2
            ;;
        --no-build)
            BUILD=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

# Keep a relative --out relative to the caller, not to the repo root.
if [[ "$OUT" != /* ]]; then
    OUT="$PWD/$OUT"
fi

cd "$ROOT"

if ! command -v xvfb-run >/dev/null 2>&1; then
    echo "screenshots: xvfb-run is required (install the 'xvfb' package)" >&2
    exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "screenshots: python3 is required for the verification step" >&2
    exit 1
fi

if [[ "$BUILD" == 1 ]]; then
    cargo build -p perseid --features screenshots --locked
fi

BIN="$ROOT/target/debug/perseid"
if [[ ! -x "$BIN" ]]; then
    echo "screenshots: $BIN is missing; run without --no-build" >&2
    exit 1
fi

# Generate into a clean temporary directory, verify, and only then replace the
# destination files. A failed run cannot leave stale or half-written docs.
TMP="$(mktemp -d "${TMPDIR:-/tmp}/perseid-screenshots.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

status=0
xvfb-run -a -s "-screen 0 3000x2000x24" \
    env LIBGL_ALWAYS_SOFTWARE=1 WINIT_X11_SCALE_FACTOR=2 RUST_LOG=info \
    timeout 90 "$BIN" \
    --screenshot-dir "$TMP" \
    --ui-scale 2 || status=$?

if [[ "$status" -ne 0 ]]; then
    if [[ "$status" -eq 124 ]]; then
        echo "screenshots: the app timed out after 90s" >&2
    else
        echo "screenshots: the app exited with status $status" >&2
    fi
    exit "$status"
fi

python3 "$ROOT/scripts/check_screenshots.py" "$TMP"

mkdir -p "$OUT"
cp "$TMP/perseid-main.png" "$TMP/perseid-memory.png" "$OUT/"
echo "Updated $OUT/perseid-main.png"
echo "Updated $OUT/perseid-memory.png"
