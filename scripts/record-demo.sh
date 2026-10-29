#!/usr/bin/env bash
# Record a real Perseid workflow inside an existing 1600x1024 X11 display.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 OUTPUT_DIRECTORY" >&2
    exit 2
fi
: "${DISPLAY:?Run inside an X11 display (for example xvfb-run)}"
command -v gifsicle >/dev/null || {
    echo 'gifsicle is required to optimize the demo GIF.' >&2
    exit 1
}
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
    --ca-dir "$work/ca" --ui-scale 1.25 >"$out/perseid.log" 2>&1 &
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
xdotool windowsize --sync "$window" 1600 1024
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
xdotool mousemove --sync 150 50
sleep 2

# Check that FFmpeg can read the X11 display and retain the frame as a useful
# failure artifact if the runner produces a blank or inaccessible capture.
ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 1 \
    -video_size 1600x1024 -i "${DISPLAY}+0,0" -frames:v 1 "$out/x11-startup.png"
[[ -s "$out/x11-startup.png" ]] || { echo 'X11 startup capture is empty.' >&2; exit 1; }

ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 30 -video_size 1600x1024 \
    -draw_mouse 1 -i "${DISPLAY}+0,0" -c:v libx264 -preset ultrafast -crf 18 \
    "$out/demo-recording.mp4" >"$out/recording.log" 2>&1 &
record_pid=$!
sleep 2

# Move in short synchronized steps so the pointer visibly leads each action
# instead of teleporting between controls. Coordinates are physical pixels in
# the 1600x1024 capture (the UI runs at 125% to keep the same logical layout).
cursor_x=150
cursor_y=50
move_cursor() {
    local target_x=$1 target_y=$2 steps=20 i x y last_x=$cursor_x last_y=$cursor_y
    for ((i = 1; i <= steps; i++)); do
        x=$((cursor_x + (target_x - cursor_x) * i / steps))
        y=$((cursor_y + (target_y - cursor_y) * i / steps))
        # --sync can wait forever if asked to move to its current coordinates.
        if ((x != last_x || y != last_y)); then
            xdotool mousemove --sync "$x" "$y"
            sleep 0.02
            last_x=$x
            last_y=$y
        fi
    done
    cursor_x=$target_x
    cursor_y=$target_y
}
click_at() {
    local x=$1 y=$2 button=${3:-1}
    move_cursor "$x" "$y"
    sleep 0.2
    xdotool click --clearmodifiers "$button"
}
open_history_menu() {
    move_cursor 520 "$1"
    sleep 0.3
    xdotool click --clearmodifiers 3
    sleep 0.8
}

# Stable physical positions in the 1600x1024 window. The history rows are
# 27 px apart starting at 218; menu item offsets were measured against the
# context menu opened from a row.
proxy_tab_x=153
comparer_tab_x=660
tab_y=63
row_2=245
row_3=272
row_5=326
menu_repeater=25
menu_intruder=58
menu_response_decoder=130
menu_response_comparer=213
# A submenu starts level with its parent menu item. Both use the same 24 px
# button height and 4 px spacing, so the second item is 35 px below it.
submenu_second_item=35

# Give first-time viewers a beat to orient on Proxy > HTTP history, then show
# real requests appearing in the table as they pass through the local proxy.
move_cursor 438 185
sleep 0.5
for path in \
    '/api/catalog' \
    '/api/export?id=1' \
    '/api/export?id=2' \
    '/api/products?id=1' \
    '/api/products?id=2'; do
    curl -fsS --max-time 10 --noproxy '' --proxy http://127.0.0.1:18080 \
        "http://127.0.0.1:18090$path" >/dev/null
    sleep 0.7
done
sleep 0.8

# Repeater: replay a captured product request with a different id.
click_at 438 "$row_5" 1
sleep 1.2
open_history_menu "$row_5"
click_at 550 $((row_5 + menu_repeater)) 1
sleep 1.3

click_at 225 256 1
sleep 0.8
xdotool key --clearmodifiers Home
for _ in {1..21}; do
    xdotool key --clearmodifiers Right
    sleep 0.02
done
xdotool key --clearmodifiers Delete
xdotool type --clearmodifiers --delay 80 '42'
sleep 0.4
click_at 60 140 1
sleep 2.2
# Hold on the JSON response, including its base64 receipt field.
move_cursor 1180 470
sleep 2.2

# Back to history, then run a three-payload Intruder attack against the export
# endpoint. Auto $$ marks only the query value.
click_at "$proxy_tab_x" "$tab_y" 1
sleep 1.0
click_at 438 "$row_3" 1
sleep 0.9
open_history_menu "$row_3"
click_at 550 $((row_3 + menu_intruder)) 1
sleep 1.3

click_at 280 149 1
sleep 1.1
click_at 1350 420 1
sleep 0.4
xdotool type --clearmodifiers --delay 60 '1'
xdotool key --clearmodifiers Return
xdotool type --clearmodifiers --delay 60 '2'
xdotool key --clearmodifiers Return
xdotool type --clearmodifiers --delay 60 '42'
sleep 0.5
click_at 344 109 1
sleep 3.0
# Select the id=42 result and show its base64 response body.
click_at 100 780 1
sleep 0.8
click_at 1052 759 1
sleep 1.0
move_cursor 1200 870
sleep 2.3

# Decoder: transfer the id=1 export response, select its base64 body, and let
# Smart decode pick the transform.
click_at "$proxy_tab_x" "$tab_y" 1
sleep 1.0
click_at 438 "$row_2" 1
sleep 0.9
open_history_menu "$row_2"
click_at 550 $((row_2 + menu_response_decoder)) 1
sleep 0.8
click_at 780 $((row_2 + menu_response_decoder + submenu_second_item)) 1
sleep 1.5

click_at 500 200 1
sleep 0.5
xdotool key --clearmodifiers ctrl+End
sleep 0.2
xdotool key --clearmodifiers shift+Home
sleep 0.3
click_at 1450 339 1
sleep 1.5
move_cursor 700 610
sleep 2.2

# Comparer: send two export responses, then compare the pair word by word.
click_at "$proxy_tab_x" "$tab_y" 1
sleep 1.0
click_at 438 "$row_2" 1
sleep 0.9
open_history_menu "$row_2"
click_at 550 $((row_2 + menu_response_comparer)) 1
sleep 0.8
click_at 438 "$row_3" 1
sleep 0.8
open_history_menu "$row_3"
click_at 550 $((row_3 + menu_response_comparer)) 1
sleep 0.8
click_at "$comparer_tab_x" "$tab_y" 1
sleep 1.2
# Both responses are already selected in table 2; select the first in table 1.
click_at 200 242 1
sleep 0.5
click_at 1530 888 1
sleep 1.5
move_cursor 800 500
sleep 3.5

# Confirm that the real UI actions, not only the initial proxy traffic, reached
# the synthetic target. Fail instead of publishing a misleading demo.
python3 - "$DEMO_REQUEST_LOG" <<'PY'
import collections
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    paths = [json.loads(line)["path"] for line in stream]
initial = [
    "/api/catalog",
    "/api/export?id=1",
    "/api/export?id=2",
    "/api/products?id=1",
    "/api/products?id=2",
]
expected = collections.Counter(initial + [
    "/api/products?id=42",
    "/api/export?id=1",
    "/api/export?id=2",
    "/api/export?id=42",
])
actual = collections.Counter(paths)
if paths[:len(initial)] != initial or actual != expected:
    raise SystemExit(
        f"Unexpected demo traffic: {paths!r}; expected prefix {initial!r} "
        f"and counts {dict(expected)!r}"
    )
PY

kill -INT "$record_pid"
wait "$record_pid" || true
record_pid=''
[[ -s "$out/demo-recording.mp4" ]] || { echo 'Recording is empty.' >&2; exit 1; }
# Downscale and reduce the frame rate before quantizing. The README only needs
# enough fidelity to follow the pointer and read the main controls; the lossy
# pass then removes dithering noise that GIF compression handles poorly.
ffmpeg -hide_banner -loglevel error -y -i "$out/demo-recording.mp4" \
    -vf 'fps=12,scale=1280:820:flags=lanczos,palettegen=max_colors=128:stats_mode=diff' \
    "$work/palette.png"
ffmpeg -hide_banner -loglevel error -y -i "$out/demo-recording.mp4" -i "$work/palette.png" \
    -filter_complex '[0:v]fps=12,scale=1280:820:flags=lanczos[video];[video][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle' \
    -loop 0 "$work/demo-unoptimized.gif"
gifsicle --optimize=3 --lossy=80 "$work/demo-unoptimized.gif" -o "$out/perseid-demo.gif"
[[ -s "$out/perseid-demo.gif" ]] || { echo 'GIF is empty.' >&2; exit 1; }
dimensions="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$out/perseid-demo.gif")"
if [[ "$dimensions" != '1280,820' ]]; then
    echo "Demo GIF must be 1280x820 (got ${dimensions:-unknown})." >&2
    exit 1
fi
echo "Demo GIF resolution: ${dimensions/,/x}"
duration="$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$out/perseid-demo.gif")"
if ! awk -v duration="$duration" 'BEGIN { exit !(duration > 0 && duration < 60) }'; then
    echo "Demo GIF must be shorter than 60 seconds (got ${duration:-unknown}s)." >&2
    exit 1
fi
size="$(wc -c < "$out/perseid-demo.gif")"
if ((size > 2500000)); then
    echo "Demo GIF must stay under 2.5 MB for the README (got ${size} bytes)." >&2
    exit 1
fi
echo "Demo GIF duration: ${duration}s"
echo "Demo GIF size: ${size} bytes"
