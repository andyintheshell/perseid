#!/usr/bin/env bash
# Record a real Perseid workflow inside an existing 1280x820 X11 display.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 OUTPUT_DIRECTORY" >&2
    exit 2
fi
: "${DISPLAY:?Run inside an X11 display (for example xvfb-run)}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$(mkdir -p "$1" && cd "$1" && pwd)"
work="$(mktemp -d)"
server_pid=''
app_pid=''
record_pid=''
cleanup() {
    for pid in "$record_pid" "$app_pid" "$server_pid"; do
        if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
    done
    rm -rf "$work"
}
trap cleanup EXIT
export DEMO_REQUEST_LOG="$out/target-requests.jsonl"
: > "$DEMO_REQUEST_LOG"
export XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data" XDG_CACHE_HOME="$work/cache"
export LIBGL_ALWAYS_SOFTWARE=1 WINIT_X11_SCALE_FACTOR=1 GDK_SCALE=1 RUST_LOG=info

python3 "$root/scripts/demo-target.py" >"$out/target.log" 2>&1 &
server_pid=$!
"$root/target/debug/perseid" --listen 127.0.0.1:18080 --no-tls-intercept \
    --ca-dir "$work/ca" --ui-scale 1 >"$out/perseid.log" 2>&1 &
app_pid=$!

# Wait for both loopback services and the actual native window to be ready.
for _ in {1..100}; do
    if ! kill -0 "$server_pid" 2>/dev/null || ! kill -0 "$app_pid" 2>/dev/null; then
        echo 'Demo process exited during startup.' >&2
        exit 1
    fi
    if (echo > /dev/tcp/127.0.0.1/18090) 2>/dev/null && (echo > /dev/tcp/127.0.0.1/18080) 2>/dev/null; then break; fi
    sleep 0.2
done
window=''
for _ in {1..100}; do
    # A window can have its title before X11 has mapped it. Focusing it in
    # that gap raises BadMatch (X_SetInputFocus), so wait for a viewable one.
    window="$(xdotool search --onlyvisible --name '^Perseid$' 2>/dev/null | head -n 1 || true)"
    [[ -n "$window" ]] && break
    sleep 0.2
done
[[ -n "$window" ]] || { echo 'Perseid window did not become visible.' >&2; exit 1; }
xdotool windowmove --sync "$window" 0 0
xdotool windowsize --sync "$window" 1280 820
focused=''
for _ in {1..20}; do
    if xdotool windowfocus --sync "$window" >"$out/x11-focus.log" 2>&1; then
        focused="$(xdotool getwindowfocus 2>/dev/null || true)"
        [[ "$focused" == "$window" ]] && break
    fi
    sleep 0.2
done
if [[ "$focused" != "$window" ]]; then
    echo "Could not focus visible Perseid window $window (focused window: ${focused:-unknown})." >&2
    cat "$out/x11-focus.log" >&2 || true
    exit 1
fi
# Xvfb starts with a large black cross-shaped cursor at the screen center.
# Move into the app before recording so its normal pointer is shown instead.
xdotool mousemove --sync 120 40
sleep 2

# Check that FFmpeg can read the X11 display and retain the frame as a useful
# failure artifact if the runner produces a blank or inaccessible capture.
ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 1 \
    -video_size 1280x820 -i "${DISPLAY}+0,0" -frames:v 1 "$out/x11-startup.png"
[[ -s "$out/x11-startup.png" ]] || { echo 'X11 startup capture is empty.' >&2; exit 1; }

ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 12 -video_size 1280x820 \
    -draw_mouse 1 -i "${DISPLAY}+0,0" -c:v libx264 -preset ultrafast -crf 18 \
    "$out/demo-recording.mp4" >"$out/recording.log" 2>&1 &
record_pid=$!
sleep 1

# Generate actual HTTP traffic through Perseid to the loopback demo service.
for path in '/api/catalog' '/api/products?id=1' '/api/products?id=2'; do
    curl -fsS --max-time 10 --noproxy '' --proxy http://127.0.0.1:18080 \
        "http://127.0.0.1:18090$path" >/dev/null
    sleep 1
done
sleep 1

# Inspect the second exchange and send it to Repeater from the context menu.
xdotool mousemove --sync 350 195 click 1
sleep 2
xdotool mousemove --sync 350 195 click 3
sleep 1
xdotool mousemove --sync 410 210 click 1
sleep 2

# Edit the actual Repeater request and send it with the documented shortcut.
xdotool mousemove --sync 180 205 click 1
xdotool key --clearmodifiers ctrl+a
xdotool type --clearmodifiers --delay 15 'GET /api/products?id=42 HTTP/1.1'
xdotool key Return
xdotool type --clearmodifiers --delay 15 'Host: 127.0.0.1:18090'
xdotool key Return Return
sleep 1
xdotool key --clearmodifiers ctrl+Return
sleep 4

# Confirm that a real direct Repeater send, not only the initial proxy traffic,
# reached the synthetic target. Fail instead of publishing a misleading demo.
python3 - "$DEMO_REQUEST_LOG" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as stream:
    paths = [json.loads(line)["path"] for line in stream]
expected = ["/api/catalog", "/api/products?id=1", "/api/products?id=2", "/api/products?id=42"]
if paths != expected:
    raise SystemExit(f"Unexpected demo traffic: {paths!r}; expected {expected!r}")
PY

kill -INT "$record_pid"
wait "$record_pid" || true
record_pid=''
[[ -s "$out/demo-recording.mp4" ]] || { echo 'Recording is empty.' >&2; exit 1; }
# Convert the recording, not staged screenshots, to a looping README-sized GIF.
ffmpeg -hide_banner -loglevel error -y -i "$out/demo-recording.mp4" \
    -vf 'fps=10,scale=1024:-1:flags=lanczos,palettegen=stats_mode=diff' "$work/palette.png"
ffmpeg -hide_banner -loglevel error -y -i "$out/demo-recording.mp4" -i "$work/palette.png" \
    -filter_complex '[0:v]fps=10,scale=1024:-1:flags=lanczos[video];[video][1:v]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle' \
    -loop 0 "$out/perseid-demo.gif"
ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$out/perseid-demo.gif"
[[ -s "$out/perseid-demo.gif" ]] || { echo 'GIF is empty.' >&2; exit 1; }
